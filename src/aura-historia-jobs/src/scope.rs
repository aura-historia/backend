use crate::jobs::WorkerQueue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum WorkerScope {
    SearchFilterProjection,
    SearchFilterPercolator,
    SearchFilterMatchNotification,
    WatchlistNotification,
    ProductListingContentAssessment,
    ProductListingTranslation,
    ProductListingEmbedding,
    ProductListingOpenSearch,
    ProductListingRawNormalization,
    NotificationDelivery,
}

impl WorkerScope {
    pub const ALL: [Self; 10] = [
        Self::SearchFilterProjection,
        Self::SearchFilterPercolator,
        Self::SearchFilterMatchNotification,
        Self::WatchlistNotification,
        Self::ProductListingContentAssessment,
        Self::ProductListingTranslation,
        Self::ProductListingEmbedding,
        Self::ProductListingOpenSearch,
        Self::ProductListingRawNormalization,
        Self::NotificationDelivery,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SearchFilterProjection => "search-filter-projection",
            Self::SearchFilterPercolator => "search-filter-percolator",
            Self::SearchFilterMatchNotification => "search-filter-match-notification",
            Self::WatchlistNotification => "watchlist-notification",
            Self::ProductListingContentAssessment => "product-content-assessment",
            Self::ProductListingTranslation => "product-translation",
            Self::ProductListingEmbedding => "product-embedding",
            Self::ProductListingOpenSearch => "product-listing-opensearch",
            Self::ProductListingRawNormalization => "product-listing-normalization",
            Self::NotificationDelivery => "notification-delivery",
        }
    }

    pub const fn router_queue_url_env(self) -> &'static str {
        match self {
            Self::SearchFilterProjection => {
                "AURA_HISTORIA_ROUTER_QUEUE_URL_SEARCH_FILTER_PROJECTION"
            }
            Self::SearchFilterPercolator => {
                "AURA_HISTORIA_ROUTER_QUEUE_URL_SEARCH_FILTER_PERCOLATOR"
            }
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
        }
    }

    pub const fn consumer_queue(self) -> WorkerQueue {
        match self {
            Self::SearchFilterProjection => WorkerQueue::SearchFilterOpenSearch,
            Self::SearchFilterPercolator => WorkerQueue::SearchFilterPercolator,
            Self::SearchFilterMatchNotification => WorkerQueue::SearchFilterMatchNotification,
            Self::WatchlistNotification => WorkerQueue::WatchlistNotification,
            Self::ProductListingContentAssessment => WorkerQueue::ProductListingContentAssessment,
            Self::ProductListingTranslation => WorkerQueue::ProductListingTranslate,
            Self::ProductListingEmbedding => WorkerQueue::ProductListingEmbed,
            Self::ProductListingOpenSearch => WorkerQueue::ProductListingOpenSearch,
            Self::ProductListingRawNormalization => WorkerQueue::ProductListingRawNormalization,
            Self::NotificationDelivery => WorkerQueue::NotificationDelivery,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn should_keep_scope_strings_and_consumer_queues() {
        let expected = [
            (
                "search-filter-projection",
                WorkerQueue::SearchFilterOpenSearch,
            ),
            (
                "search-filter-percolator",
                WorkerQueue::SearchFilterPercolator,
            ),
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
        ];
        assert_eq!(
            WorkerScope::ALL,
            WorkerScope::iter().collect::<Vec<_>>().as_slice()
        );
        for (scope, (wire, queue)) in WorkerScope::ALL.into_iter().zip(expected) {
            assert_eq!(wire, scope.as_str());
            assert_eq!(queue, scope.consumer_queue());
            assert_eq!(Some(scope), queue.scope());
        }
        assert_eq!(None, WorkerQueue::UserTierEnforcement.scope());
    }
}
