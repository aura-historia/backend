use crate::product_listings::product_data::ProductListingImageData;
use crate::values::{LocalizedTextData, PriceData};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    ProductListingAddress, ProductListingAuction, ProductListingEventPayload,
    ProductListingPricing, ProductSaleValuation,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_service::use_cases::ProductListingEvent;
use serde::Serialize;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingEventData {
    event_type: &'static str,
    product_listing_id: ProductListingId,
    event_id: EventId,
    payload: ProductListingEventPayloadData,
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
enum ProductListingEventPayloadData {
    Created(ProductListingCreatedHistoryPayloadData),
    AvailabilityChanged(ListingAvailabilityChangedHistoryPayloadData),
    AddressChanged(ProductListingAddressChangedHistoryPayloadData),
    PriceChanged(ProductListingPriceChangedHistoryPayloadData),
    UrlChanged(ProductListingUrlChangedHistoryPayloadData),
    ImagesChanged(ProductListingImagesChangedHistoryPayloadData),
    AuctionChanged(ProductListingAuctionChangedHistoryPayloadData),
    Withdrawn(ProductListingWithdrawnHistoryPayloadData),
    Restored(ProductListingRestoredHistoryPayloadData),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingCreatedHistoryPayloadData {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_address: Option<GeoAddressData>,
    pricing: ProductListingPricingData,
    #[serde(skip_serializing_if = "Option::is_none")]
    sale_valuation: Option<ProductSaleValuationData>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::listing_availability::option"
    )]
    availability: Option<ListingAvailability>,
    url: Url,
    images: Vec<ProductListingImageData>,
    auction: ProductListingAuctionData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListingAvailabilityChangedHistoryPayloadData {
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::listing_availability::option"
    )]
    previous: Option<ListingAvailability>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::listing_availability::option"
    )]
    current: Option<ListingAvailability>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingAddressChangedHistoryPayloadData {
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_address: Option<GeoAddressData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPriceChangedHistoryPayloadData {
    old_pricing: ProductListingPricingData,
    new_pricing: ProductListingPricingData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingUrlChangedHistoryPayloadData {
    old_url: Url,
    new_url: Url,
}

#[derive(Debug, Serialize)]
struct ProductListingImagesChangedHistoryPayloadData {
    images: Vec<ProductListingImageData>,
}

#[derive(Debug, Serialize)]
struct ProductListingAuctionChangedHistoryPayloadData {
    auction: ProductListingAuctionData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingWithdrawnHistoryPayloadData {
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::listing_availability::option"
    )]
    previous_availability: Option<ListingAvailability>,
}

#[derive(Debug, Serialize)]
struct ProductListingRestoredHistoryPayloadData {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingData {
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
struct ProductListingAuctionData {
    #[serde(with = "time::serde::rfc3339::option")]
    start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    end: Option<OffsetDateTime>,
}

impl From<ProductListingEvent> for ProductListingEventData {
    fn from(event: ProductListingEvent) -> Self {
        Self {
            event_type: event.event_type(),
            product_listing_id: event.product_listing_id,
            event_id: event.event_id,
            payload: event.payload.into(),
            timestamp: event.timestamp,
        }
    }
}

impl From<ProductListingEventPayload> for ProductListingEventPayloadData {
    fn from(value: ProductListingEventPayload) -> Self {
        match value {
            ProductListingEventPayload::Created(value) => {
                Self::Created(ProductListingCreatedHistoryPayloadData {
                    title: value.title.map(Into::into),
                    description: value.description.map(Into::into),
                    structured_address: value.address.structured.map(Into::into),
                    geo_address: value.address.geo.map(Into::into),
                    pricing: value.pricing.into(),
                    sale_valuation: value.sale_valuation.map(Into::into),
                    availability: value.availability,
                    url: value.url,
                    images: images(value.images),
                    auction: value.auction.into(),
                })
            }
            ProductListingEventPayload::AvailabilityChanged(value) => {
                Self::AvailabilityChanged(ListingAvailabilityChangedHistoryPayloadData {
                    previous: value.previous,
                    current: value.current,
                })
            }
            ProductListingEventPayload::AddressChanged(value) => Self::AddressChanged(
                ProductListingAddressChangedHistoryPayloadData::from(value.address),
            ),
            ProductListingEventPayload::PriceChanged(value) => {
                Self::PriceChanged(ProductListingPriceChangedHistoryPayloadData {
                    old_pricing: value.old_pricing.into(),
                    new_pricing: value.new_pricing.into(),
                })
            }
            ProductListingEventPayload::UrlChanged(value) => {
                Self::UrlChanged(ProductListingUrlChangedHistoryPayloadData {
                    old_url: value.old_url,
                    new_url: value.new_url,
                })
            }
            ProductListingEventPayload::ImagesChanged(value) => {
                Self::ImagesChanged(ProductListingImagesChangedHistoryPayloadData {
                    images: images(value.images),
                })
            }
            ProductListingEventPayload::AuctionChanged(value) => {
                Self::AuctionChanged(ProductListingAuctionChangedHistoryPayloadData {
                    auction: value.auction.into(),
                })
            }
            ProductListingEventPayload::Withdrawn(value) => {
                Self::Withdrawn(ProductListingWithdrawnHistoryPayloadData {
                    previous_availability: value.previous_availability,
                })
            }
            ProductListingEventPayload::Restored(_) => {
                Self::Restored(ProductListingRestoredHistoryPayloadData {})
            }
        }
    }
}

impl From<ProductListingAddress> for ProductListingAddressChangedHistoryPayloadData {
    fn from(address: ProductListingAddress) -> Self {
        Self {
            structured_address: address.structured.map(Into::into),
            geo_address: address.geo.map(Into::into),
        }
    }
}

impl From<ProductListingPricing> for ProductListingPricingData {
    fn from(pricing: ProductListingPricing) -> Self {
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

impl From<ProductListingAuction> for ProductListingAuctionData {
    fn from(auction: ProductListingAuction) -> Self {
        Self {
            start: auction.start,
            end: auction.end,
        }
    }
}

fn images(images: impl IntoIterator<Item = ProductListingImage>) -> Vec<ProductListingImageData> {
    images
        .into_iter()
        .map(ProductListingImageData::from)
        .collect()
}
