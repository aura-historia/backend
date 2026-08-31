use crate::ports::ListingSourceSummaryWithReferral;
use application::error::BoxError;
use listing_source_core::ListingSourceId;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceSummaryReadError {
    #[error("listing source summary query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("listing source summary read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

/// Resolves ListingSource presentation needed by ProductListing result pages.
#[async_trait::async_trait]
pub trait ListingSourceSummaryReader: Send + Sync {
    async fn find_summaries(
        &self,
        listing_source_ids: &[ListingSourceId],
    ) -> Result<
        HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
        ListingSourceSummaryReadError,
    >;
}
