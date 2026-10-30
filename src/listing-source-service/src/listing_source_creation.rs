use crate::ports::{
    ListingSourceIngestionConfigurations, ListingSourceRepository, ListingSourceRepositoryError,
    StoredListingSource,
};
use listing_source_core::{ListingSource, NewListingSource};

/// Persists a new ListingSource with its ingestion configuration in the caller's transaction.
pub async fn insert_new_listing_source(
    sources: &mut impl ListingSourceRepository,
    input: NewListingSource,
    configuration: &ListingSourceIngestionConfigurations,
) -> Result<StoredListingSource, ListingSourceRepositoryError> {
    sources
        .insert(&ListingSource::create(input), configuration)
        .await
}
