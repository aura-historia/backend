use application::error::BoxError;
use domain_primitives::event_id::EventId;
use localization::Language;
use money::Price;
use product_listing_core::{
    content_policy::ContentPolicyDecision, listing_availability::ListingAvailability,
    product_listing_id::ProductListingId, product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId, shop_listing_id::ShopListingId, title::Title,
};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingWatchlistNotificationSource {
    pub event_id: EventId,
    pub event_time: OffsetDateTime,
    pub product_listing_id: ProductListingId,
    pub product_listing_slug_id: ProductListingSlugId,
    pub shop_id: ShopId,
    pub shop_listing_id: ShopListingId,
    pub shop_slug_id: ShopSlugId,
    pub shop_name: ShopName,
    pub title: Option<HashMap<Language, Title>>,
    pub image: Option<ProductListingImage>,
    pub content_policy: Option<ContentPolicyDecision>,
    pub url: Url,
    pub view_url: Url,
    pub change: ProductListingWatchlistNotificationChange,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingWatchlistNotificationChange {
    PriceChanged {
        old_price: Option<Price>,
        new_price: Option<Price>,
    },
    AvailabilityChanged {
        old_availability: Option<ListingAvailability>,
        new_availability: Option<ListingAvailability>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingWatchlistNotificationSourceReadError {
    #[error("watchlist notification source query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist notification source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingWatchlistNotificationSourceReader: Send {
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<
        Option<ProductListingWatchlistNotificationSource>,
        ProductListingWatchlistNotificationSourceReadError,
    >;
}

pub trait ProductListingWatchlistNotificationSourceReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingWatchlistNotificationSourceReader + 'tx;
}
