//! Composes the production handler of each consumed scope.

use crate::{scope::ConsumedScope, scopes};
use application::error::BoxError;
use aura_historia_jobs::WorkerScope;
use platform_worker::JobHandler;
use sqlx::PgPool;
use std::sync::Arc;

/// A scope whose production use case could not be composed, typically for missing provider
/// configuration. The worker does not start without it.
#[derive(Debug, thiserror::Error)]
#[error("failed to compose the {scope} job handler")]
pub struct CompositionError {
    pub scope: String,
    #[source]
    pub source: BoxError,
}

impl CompositionError {
    pub fn new(scope: &str, source: impl Into<BoxError>) -> Self {
        Self {
            scope: scope.to_owned(),
            source: source.into(),
        }
    }
}

/// The scope's handler over the shared process pool. Constructing it performs no provider
/// request; provider settings are read and validated here, so a misconfigured scope fails
/// startup rather than its first job.
pub async fn compose_handler(
    scope: &ConsumedScope,
    pool: &PgPool,
) -> Result<Arc<dyn JobHandler>, CompositionError> {
    match scope {
        ConsumedScope::Worker(worker) => match worker {
            WorkerScope::SearchFilterMatchNotification => {
                scopes::search_filter_match_notification::compose(pool).await
            }
            WorkerScope::WatchlistNotification => {
                scopes::watchlist_notification::compose(pool).await
            }
            WorkerScope::ProductListingContentAssessment => {
                scopes::product_content_assessment::compose(pool).await
            }
            WorkerScope::ProductListingTranslation => {
                scopes::product_translation::compose(pool).await
            }
            WorkerScope::ProductListingEmbedding => scopes::product_embedding::compose(pool).await,
            WorkerScope::ProductListingOpenSearch => {
                scopes::product_listing_opensearch::compose_live(pool).await
            }
            WorkerScope::ProductListingRawNormalization => {
                scopes::product_listing_normalization::compose(pool).await
            }
            WorkerScope::NotificationDelivery => scopes::notification_delivery::compose(pool).await,
            WorkerScope::MarketingConsentSync => {
                scopes::marketing_consent_sync::compose(pool).await
            }
            WorkerScope::GeoResolution => scopes::geo_resolution::compose(pool).await,
            WorkerScope::ListingGeography => scopes::listing_geography::compose(pool).await,
            WorkerScope::ListingShipping => scopes::listing_shipping::compose(pool).await,
            WorkerScope::UserIdentityDeletion => {
                scopes::user_identity_deletion::compose(pool).await
            }
        },
        ConsumedScope::ProductListingIngestion => {
            scopes::product_listing_ingestion::compose(pool).await
        }
        ConsumedScope::ProjectionGeneration(generation) => {
            scopes::product_listing_opensearch::compose_generation(pool, generation).await
        }
    }
}
