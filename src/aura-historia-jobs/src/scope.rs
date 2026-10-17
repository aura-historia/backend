use crate::jobs::WorkerQueue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum WorkerScope {
    SearchFilterMatchNotification,
    WatchlistNotification,
    ProductListingContentAssessment,
    ProductListingTranslation,
    ProductListingEmbedding,
    ProductListingOpenSearch,
    ProductListingRawNormalization,
    NotificationDelivery,
    MarketingConsentSync,
    GeoResolution,
    ListingGeography,
    ListingShipping,
}

impl WorkerScope {
    pub const ALL: [Self; 12] = [
        Self::SearchFilterMatchNotification,
        Self::WatchlistNotification,
        Self::ProductListingContentAssessment,
        Self::ProductListingTranslation,
        Self::ProductListingEmbedding,
        Self::ProductListingOpenSearch,
        Self::ProductListingRawNormalization,
        Self::NotificationDelivery,
        Self::MarketingConsentSync,
        Self::GeoResolution,
        Self::ListingGeography,
        Self::ListingShipping,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SearchFilterMatchNotification => "search-filter-match-notification",
            Self::WatchlistNotification => "watchlist-notification",
            Self::ProductListingContentAssessment => "product-content-assessment",
            Self::ProductListingTranslation => "product-translation",
            Self::ProductListingEmbedding => "product-embedding",
            Self::ProductListingOpenSearch => "product-listing-opensearch",
            Self::ProductListingRawNormalization => "product-listing-normalization",
            Self::NotificationDelivery => "notification-delivery",
            Self::MarketingConsentSync => "marketing-consent-sync",
            Self::GeoResolution => "geo-resolution",
            Self::ListingGeography => "listing-geography",
            Self::ListingShipping => "listing-shipping",
        }
    }

    pub const fn router_queue_url_env(self) -> &'static str {
        match self {
            Self::SearchFilterMatchNotification => {
                "AURA_HISTORIA_ROUTER_QUEUE_URL_SEARCH_FILTER_MATCH_NOTIFICATION"
            }
            Self::WatchlistNotification => "AURA_HISTORIA_ROUTER_QUEUE_URL_WATCHLIST_NOTIFICATION",
            Self::ProductListingContentAssessment => {
                "AURA_HISTORIA_ROUTER_QUEUE_URL_PRODUCT_LISTING_CONTENT_ASSESSMENT"
            }
            Self::ProductListingTranslation => {
                "AURA_HISTORIA_ROUTER_QUEUE_URL_PRODUCT_LISTING_TRANSLATION"
            }
            Self::ProductListingEmbedding => {
                "AURA_HISTORIA_ROUTER_QUEUE_URL_PRODUCT_LISTING_EMBEDDING"
            }
            Self::ProductListingOpenSearch => {
                "AURA_HISTORIA_ROUTER_QUEUE_URL_PRODUCT_LISTING_OPENSEARCH"
            }
            Self::ProductListingRawNormalization => {
                "AURA_HISTORIA_ROUTER_QUEUE_URL_PRODUCT_LISTING_RAW_NORMALIZATION"
            }
            Self::NotificationDelivery => "AURA_HISTORIA_ROUTER_QUEUE_URL_NOTIFICATION_DELIVERY",
            Self::MarketingConsentSync => "AURA_HISTORIA_ROUTER_QUEUE_URL_MARKETING_CONSENT_SYNC",
            Self::GeoResolution => "AURA_HISTORIA_ROUTER_QUEUE_URL_GEO_RESOLUTION",
            Self::ListingGeography => "AURA_HISTORIA_ROUTER_QUEUE_URL_LISTING_GEOGRAPHY",
            Self::ListingShipping => "AURA_HISTORIA_ROUTER_QUEUE_URL_LISTING_SHIPPING",
        }
    }

    pub const fn consumer_queue(self) -> WorkerQueue {
        match self {
            Self::SearchFilterMatchNotification => WorkerQueue::SearchFilterMatchNotification,
            Self::WatchlistNotification => WorkerQueue::WatchlistNotification,
            Self::ProductListingContentAssessment => WorkerQueue::ProductListingContentAssessment,
            Self::ProductListingTranslation => WorkerQueue::ProductListingTranslate,
            Self::ProductListingEmbedding => WorkerQueue::ProductListingEmbed,
            Self::ProductListingOpenSearch => WorkerQueue::ProductListingOpenSearch,
            Self::ProductListingRawNormalization => WorkerQueue::ProductListingRawNormalization,
            Self::NotificationDelivery => WorkerQueue::NotificationDelivery,
            Self::MarketingConsentSync => WorkerQueue::MarketingConsentSync,
            Self::GeoResolution => WorkerQueue::GeoResolution,
            Self::ListingGeography => WorkerQueue::ListingGeography,
            Self::ListingShipping => WorkerQueue::ListingShipping,
        }
    }

    pub const fn queue_type(self) -> WorkerQueueType {
        match self {
            Self::MarketingConsentSync => WorkerQueueType::Fifo,
            _ => WorkerQueueType::Standard,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerQueueType {
    Standard,
    Fifo,
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn should_keep_scope_strings_and_consumer_queues() {
        let expected = [
            (
                "search-filter-match-notification",
                WorkerQueue::SearchFilterMatchNotification,
            ),
            ("watchlist-notification", WorkerQueue::WatchlistNotification),
            (
                "product-content-assessment",
                WorkerQueue::ProductListingContentAssessment,
            ),
            ("product-translation", WorkerQueue::ProductListingTranslate),
            ("product-embedding", WorkerQueue::ProductListingEmbed),
            (
                "product-listing-opensearch",
                WorkerQueue::ProductListingOpenSearch,
            ),
            (
                "product-listing-normalization",
                WorkerQueue::ProductListingRawNormalization,
            ),
            ("notification-delivery", WorkerQueue::NotificationDelivery),
            ("marketing-consent-sync", WorkerQueue::MarketingConsentSync),
            ("geo-resolution", WorkerQueue::GeoResolution),
            ("listing-geography", WorkerQueue::ListingGeography),
            ("listing-shipping", WorkerQueue::ListingShipping),
        ];
        assert_eq!(12, WorkerScope::ALL.len());
        assert_eq!(expected.len(), WorkerScope::ALL.len());
        assert_eq!(
            WorkerScope::ALL,
            WorkerScope::iter().collect::<Vec<_>>().as_slice()
        );
        for (scope, (wire, queue)) in WorkerScope::ALL.into_iter().zip(expected) {
            assert_eq!(wire, scope.as_str());
            assert_eq!(queue, scope.consumer_queue());
            assert_eq!(Some(scope), queue.scope());
        }
        assert_eq!(WorkerQueue::ALL.len(), WorkerScope::ALL.len());
        for queue in WorkerQueue::ALL {
            assert!(queue.scope().is_some());
        }
        for removed in ["search-filter-projection", "search-filter-percolator"] {
            assert!(
                WorkerScope::ALL
                    .into_iter()
                    .all(|scope| scope.as_str() != removed)
            );
        }
        assert_eq!(None, WorkerQueue::UserTierEnforcement.scope());
        assert_eq!(
            WorkerQueueType::Fifo,
            WorkerScope::MarketingConsentSync.queue_type()
        );
        assert_eq!(
            "AURA_HISTORIA_ROUTER_QUEUE_URL_MARKETING_CONSENT_SYNC",
            WorkerScope::MarketingConsentSync.router_queue_url_env()
        );
        for scope in WorkerScope::ALL
            .into_iter()
            .filter(|scope| *scope != WorkerScope::MarketingConsentSync)
        {
            assert_eq!(WorkerQueueType::Standard, scope.queue_type());
        }
    }
}
