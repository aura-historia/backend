//! The lease and retry budget of each worker scope, carried over from the SQS queues they
//! replaced, and the PostgreSQL job queue scopes built from them.
pub mod job_queue;

use aura_historia_jobs::WorkerScope;
use std::time::Duration;

/// Attempts before a worker job moves to failure custody; the SQS `maxReceiveCount` it replaced.
pub const MAX_RECEIVE_COUNT: u32 = 5;

/// A worker scope's lease and retry delay; the visibility timeout of the SQS queue it replaced.
pub fn visibility(scope: WorkerScope) -> Duration {
    Duration::from_secs(match scope {
        WorkerScope::ProductListingOpenSearch
        | WorkerScope::SearchFilterMatchNotification
        | WorkerScope::WatchlistNotification
        | WorkerScope::GeoResolution
        | WorkerScope::ListingGeography
        | WorkerScope::ListingShipping
        | WorkerScope::ProductListingTranslation => 300,
        WorkerScope::MarketingConsentSync | WorkerScope::UserIdentityDeletion => 330,
        WorkerScope::ProductListingContentAssessment
        | WorkerScope::ProductListingRawNormalization => 270,
        WorkerScope::ProductListingEmbedding => 360,
        WorkerScope::NotificationDelivery => 330,
    })
}

#[cfg(test)]
mod tests {
    use super::visibility;
    use aura_historia_jobs::WorkerScope;
    use std::time::Duration;

    #[test]
    fn should_keep_each_scope_name_and_visibility_stable() {
        let expected = [
            ("search-filter-match-notification", 300),
            ("watchlist-notification", 300),
            ("product-content-assessment", 270),
            ("product-translation", 300),
            ("product-embedding", 360),
            ("product-listing-opensearch", 300),
            ("product-listing-normalization", 270),
            ("notification-delivery", 330),
            ("marketing-consent-sync", 330),
            ("geo-resolution", 300),
            ("listing-geography", 300),
            ("listing-shipping", 300),
            ("user-identity-deletion", 330),
        ];
        assert_eq!(WorkerScope::ALL.len(), expected.len());
        for (scope, (name, seconds)) in WorkerScope::ALL.into_iter().zip(expected) {
            assert_eq!(name, scope.as_str());
            assert_eq!(Duration::from_secs(seconds), visibility(scope), "{name}");
        }
    }
}
