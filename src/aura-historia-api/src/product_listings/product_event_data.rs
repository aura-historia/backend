use crate::values::{LocalizedTextData, PriceData};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;
use product_listing_core::listing_availability::ListingAvailability;

use product_listing_core::product_listing::{
    ListingSaleObservation, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_event::{
    ListingSaleObservationChange, ProductListingChanged, ProductListingDiscovered,
    ProductListingEventPayload, ProductListingLifecycleChange, ValueChange,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_service::use_cases::ProductListingEvent;
use serde::{Serialize, Serializer};
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
enum ProductListingEventPayloadData {
    Discovered(ProductListingDiscoveredHistoryPayloadData),
    Changed(ProductListingChangedHistoryPayloadData),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingDiscoveredHistoryPayloadData {
    listing_source_id: ListingSourceId,
    #[serde(serialize_with = "crate::wire::source_listing_id::serialize")]
    source_listing_id: SourceListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<LocalizedTextData>,
    pricing: ProductListingPricingData,
    #[serde(with = "crate::wire::listing_availability::option")]
    availability: Option<ListingAvailability>,
    url: Url,
    image_count: usize,
    auction: ProductListingAuctionData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingChangedHistoryPayloadData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<ValueChangeData<Option<PriceData>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<ValueChangeData<Option<PriceData>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<ValueChangeData<Option<PriceData>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    availability: Option<ListingAvailabilityChangeData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<ValueChangeData<Url>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_count: Option<ValueChangeData<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auction: Option<ValueChangeData<ProductListingAuctionData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lifecycle: Option<ProductListingLifecycleChangeData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sale_observation: Option<ListingSaleObservationChangeData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValueChangeData<T> {
    previous: T,
    current: T,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListingAvailabilityChangeData {
    #[serde(with = "crate::wire::listing_availability::option")]
    previous: Option<ListingAvailability>,
    #[serde(with = "crate::wire::listing_availability::option")]
    current: Option<ListingAvailability>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingLifecycleChangeData {
    transition: &'static str,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_previous_availability"
    )]
    previous_availability: Option<Option<ListingAvailability>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListingSaleObservationChangeData {
    transition: &'static str,
    observation: ListingSaleObservationHistoryPayloadData,
}

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
struct ListingSaleObservationHistoryPayloadData {
    #[serde(with = "time::serde::rfc3339")]
    observed_at: OffsetDateTime,
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
            ProductListingEventPayload::Discovered(value) => Self::Discovered(value.into()),
            ProductListingEventPayload::Changed(value) => Self::Changed(value.into()),
        }
    }
}

impl From<ProductListingDiscovered> for ProductListingDiscoveredHistoryPayloadData {
    fn from(value: ProductListingDiscovered) -> Self {
        Self {
            listing_source_id: value.listing_source_id(),
            source_listing_id: value.source_listing_id().clone(),
            title: value.title().cloned().map(Into::into),
            description: value.description().cloned().map(Into::into),
            pricing: value.pricing().into(),
            availability: value.availability(),
            url: value.url().clone(),
            image_count: value.image_count(),
            auction: value.auction().into(),
        }
    }
}

impl From<ProductListingChanged> for ProductListingChangedHistoryPayloadData {
    fn from(value: ProductListingChanged) -> Self {
        Self {
            price: value.price().map(price_change),
            price_estimate_min: value.price_estimate_min().map(price_change),
            price_estimate_max: value.price_estimate_max().map(price_change),
            availability: value
                .availability()
                .map(ListingAvailabilityChangeData::from),
            url: value.url().map(ValueChangeData::from),
            image_count: value.image_count().map(ValueChangeData::from),
            auction: value.auction().map(auction_change),
            lifecycle: value.lifecycle().map(Into::into),
            sale_observation: value.sale_observation().map(Into::into),
        }
    }
}

impl<T: Clone> From<&ValueChange<T>> for ValueChangeData<T> {
    fn from(value: &ValueChange<T>) -> Self {
        Self {
            previous: value.previous().clone(),
            current: value.current().clone(),
        }
    }
}

impl From<&ValueChange<Option<ListingAvailability>>> for ListingAvailabilityChangeData {
    fn from(value: &ValueChange<Option<ListingAvailability>>) -> Self {
        Self {
            previous: *value.previous(),
            current: *value.current(),
        }
    }
}

fn serialize_previous_availability<S>(
    value: &Option<Option<ListingAvailability>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(value) => crate::wire::listing_availability::option::serialize(value, serializer),
        None => serializer.serialize_none(),
    }
}

impl From<&ProductListingLifecycleChange> for ProductListingLifecycleChangeData {
    fn from(value: &ProductListingLifecycleChange) -> Self {
        match value {
            ProductListingLifecycleChange::Withdrawn {
                previous_availability,
            } => Self {
                transition: "WITHDRAWN",
                previous_availability: Some(*previous_availability),
            },
            ProductListingLifecycleChange::Restored => Self {
                transition: "RESTORED",
                previous_availability: None,
            },
        }
    }
}

impl From<&ListingSaleObservationChange> for ListingSaleObservationChangeData {
    fn from(value: &ListingSaleObservationChange) -> Self {
        match value {
            ListingSaleObservationChange::Observed(observation) => Self {
                transition: "OBSERVED",
                observation: (*observation).into(),
            },
            ListingSaleObservationChange::Retracted(observation) => Self {
                transition: "RETRACTED",
                observation: (*observation).into(),
            },
        }
    }
}

fn price_change(value: &ValueChange<Option<money::Price>>) -> ValueChangeData<Option<PriceData>> {
    ValueChangeData {
        previous: (*value.previous()).map(Into::into),
        current: (*value.current()).map(Into::into),
    }
}

fn auction_change(
    value: &ValueChange<ProductListingAuction>,
) -> ValueChangeData<ProductListingAuctionData> {
    ValueChangeData {
        previous: (*value.previous()).into(),
        current: (*value.current()).into(),
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

impl From<ListingSaleObservation> for ListingSaleObservationHistoryPayloadData {
    fn from(observation: ListingSaleObservation) -> Self {
        Self {
            observed_at: observation.observed_at(),
            fx_rate_id: observation.fx_rate_id(),
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
