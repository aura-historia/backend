use crate::product_listings::product_data::ProductListingAuctionData;
use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing::ListingSaleObservation,
    product_listing_id::ProductListingId, source_listing_id::SourceListingId,
};
use product_listing_service::use_cases::{
    ProductListingDiscoveryHistory, ProductListingHistoryChange, ProductListingHistoryEntry,
    ProductListingHistoryEntryKind, ProductListingHistoryPrice, ProductListingHistoryPriceDisplay,
    ProductListingHistoryPricing,
};
use serde::Serialize;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingHistoryEntryData {
    event_type: &'static str,
    product_listing_id: ProductListingId,
    event_id: EventId,
    payload: ProductListingHistoryPayloadData,
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ProductListingHistoryPayloadData {
    Discovered(Box<ProductListingDiscoveryHistoryData>),
    Changed(ProductListingChangedHistoryData),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingDiscoveryHistoryData {
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
    image_count: u64,
    auction: Option<ProductListingAuctionData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingChangedHistoryData {
    changes: Vec<ProductListingHistoryChangeData>,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
enum ProductListingHistoryChangeData {
    DeliveryChanged,
    MainPriceChanged {
        previous: Option<ProductListingHistoryPriceData<ProductListingPriceData>>,
        current: Option<ProductListingHistoryPriceData<ProductListingPriceData>>,
    },
    MinimumEstimateChanged {
        previous: Option<ProductListingHistoryPriceData<PriceData>>,
        current: Option<ProductListingHistoryPriceData<PriceData>>,
    },
    MaximumEstimateChanged {
        previous: Option<ProductListingHistoryPriceData<PriceData>>,
        current: Option<ProductListingHistoryPriceData<PriceData>>,
    },
    AvailabilityChanged {
        #[serde(with = "crate::wire::listing_availability::option")]
        previous: Option<ListingAvailability>,
        #[serde(with = "crate::wire::listing_availability::option")]
        current: Option<ListingAvailability>,
    },
    UrlChanged {
        previous: Url,
        current: Url,
    },
    ImagesChanged {
        previous_count: u64,
        current_count: u64,
    },
    AuctionChanged {
        previous: Option<ProductListingAuctionData>,
        current: Option<ProductListingAuctionData>,
    },
    Withdrawn {
        #[serde(with = "crate::wire::listing_availability::option")]
        previous_availability: Option<ListingAvailability>,
    },
    Restored,
    SaleObserved {
        observation: ListingSaleObservationHistoryData,
    },
    SaleObservationRetracted {
        observation: ListingSaleObservationHistoryData,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<ProductListingHistoryPriceData<ProductListingPriceData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<ProductListingHistoryPriceData<PriceData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<ProductListingHistoryPriceData<PriceData>>,
}

#[derive(Debug, Serialize)]
struct ProductListingHistoryPriceData<T> {
    #[serde(flatten)]
    source: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<ProductListingHistoryPriceDisplayData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingHistoryPriceDisplayData {
    #[serde(flatten)]
    price: PriceData,
    fx_rate_id: FxRateId,
    #[serde(with = "time::serde::rfc3339")]
    captured_at: OffsetDateTime,
}

impl<T, D: From<T>> From<ProductListingHistoryPrice<T>> for ProductListingHistoryPriceData<D> {
    fn from(value: ProductListingHistoryPrice<T>) -> Self {
        Self {
            source: value.source.into(),
            display: value.display.map(Into::into),
        }
    }
}

impl From<ProductListingHistoryPriceDisplay> for ProductListingHistoryPriceDisplayData {
    fn from(display: ProductListingHistoryPriceDisplay) -> Self {
        Self {
            price: display.price.into(),
            fx_rate_id: display.fx_rate_id,
            captured_at: display.captured_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListingSaleObservationHistoryData {
    #[serde(with = "time::serde::rfc3339")]
    observed_at: OffsetDateTime,
    fx_rate_id: FxRateId,
}

impl From<ProductListingHistoryEntry> for ProductListingHistoryEntryData {
    fn from(entry: ProductListingHistoryEntry) -> Self {
        Self {
            event_type: entry.event_type().as_str(),
            product_listing_id: entry.product_listing_id,
            event_id: entry.event_id,
            payload: entry.kind.into(),
            timestamp: entry.occurred_at,
        }
    }
}

impl From<ProductListingHistoryEntryKind> for ProductListingHistoryPayloadData {
    fn from(value: ProductListingHistoryEntryKind) -> Self {
        match value {
            ProductListingHistoryEntryKind::Discovered(discovery) => {
                Self::Discovered(Box::new((*discovery).into()))
            }
            ProductListingHistoryEntryKind::Changed(changes) => {
                Self::Changed(ProductListingChangedHistoryData {
                    changes: changes.into_inner().into_iter().map(Into::into).collect(),
                })
            }
        }
    }
}

impl From<ProductListingDiscoveryHistory> for ProductListingDiscoveryHistoryData {
    fn from(value: ProductListingDiscoveryHistory) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            source_listing_id: value.source_listing_id,
            title: value.title.map(Into::into),
            description: value.description.map(Into::into),
            pricing: value.pricing.into(),
            availability: value.availability,
            url: value.url,
            image_count: value.image_count,
            auction: value.auction.map(Into::into),
        }
    }
}

impl From<ProductListingHistoryChange> for ProductListingHistoryChangeData {
    fn from(value: ProductListingHistoryChange) -> Self {
        match value {
            ProductListingHistoryChange::DeliveryChanged => Self::DeliveryChanged,
            ProductListingHistoryChange::MainPriceChanged { previous, current } => {
                Self::MainPriceChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::MinimumEstimateChanged { previous, current } => {
                Self::MinimumEstimateChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::MaximumEstimateChanged { previous, current } => {
                Self::MaximumEstimateChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::AvailabilityChanged { previous, current } => {
                Self::AvailabilityChanged { previous, current }
            }
            ProductListingHistoryChange::UrlChanged { previous, current } => {
                Self::UrlChanged { previous, current }
            }
            ProductListingHistoryChange::ImagesChanged {
                previous_count,
                current_count,
            } => Self::ImagesChanged {
                previous_count,
                current_count,
            },
            ProductListingHistoryChange::AuctionChanged { previous, current } => {
                Self::AuctionChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::Withdrawn {
                previous_availability,
            } => Self::Withdrawn {
                previous_availability,
            },
            ProductListingHistoryChange::Restored => Self::Restored,
            ProductListingHistoryChange::SaleObserved { observation } => Self::SaleObserved {
                observation: observation.into(),
            },
            ProductListingHistoryChange::SaleObservationRetracted { observation } => {
                Self::SaleObservationRetracted {
                    observation: observation.into(),
                }
            }
        }
    }
}

impl From<ProductListingHistoryPricing> for ProductListingPricingData {
    fn from(pricing: ProductListingHistoryPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<ListingSaleObservation> for ListingSaleObservationHistoryData {
    fn from(observation: ListingSaleObservation) -> Self {
        Self {
            observed_at: observation.observed_at(),
            fx_rate_id: observation.fx_rate_id(),
        }
    }
}

#[cfg(test)]
mod tests;
