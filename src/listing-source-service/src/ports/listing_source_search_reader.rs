use application::error::BoxError;

use crate::use_cases::queries::search_listing_sources::{
    SearchListingSourcesRequest, SearchListingSourcesResult,
};

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceSearchReadError {
    #[error("temporary listing source search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid listing source search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListingSourceSearchReader: Send {
    async fn search(
        &mut self,
        request: &SearchListingSourcesRequest,
    ) -> Result<SearchListingSourcesResult, ListingSourceSearchReadError>;
}

pub trait ListingSourceSearchReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ListingSourceSearchReader + 'tx;
}
