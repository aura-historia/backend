use crate::OperationError;
use listing_source_service::use_cases::queries::{
    get_public_listing_source_by_slug::GetPublicListingSourceBySlugError,
    search_public_listing_sources::SearchPublicListingSourcesError,
};
use party_service::discovery::DiscoverPartyLocationsError;
use product_listing_service::use_cases::GetPublicCatalogueOverviewError;

pub(super) fn search(error: SearchPublicListingSourcesError) -> OperationError {
    match error {
        error @ (SearchPublicListingSourcesError::TemporarilyUnavailable { .. }
        | SearchPublicListingSourcesError::BeginTransaction { .. }
        | SearchPublicListingSourcesError::CommitTransaction { .. }) => {
            OperationError::Unavailable(Box::new(error))
        }
        error @ (SearchPublicListingSourcesError::InvalidReadModel { .. }
        | SearchPublicListingSourcesError::Internal { .. }) => {
            OperationError::Internal(Box::new(error))
        }
    }
}

pub(super) fn detail(error: GetPublicListingSourceBySlugError) -> OperationError {
    match error {
        GetPublicListingSourceBySlugError::NotFound => OperationError::NotFound,
        error @ (GetPublicListingSourceBySlugError::TemporarilyUnavailable { .. }
        | GetPublicListingSourceBySlugError::BeginTransaction { .. }
        | GetPublicListingSourceBySlugError::CommitTransaction { .. }) => {
            OperationError::Unavailable(Box::new(error))
        }
        error @ (GetPublicListingSourceBySlugError::InvalidReadModel { .. }
        | GetPublicListingSourceBySlugError::Internal { .. }) => {
            OperationError::Internal(Box::new(error))
        }
    }
}

pub(super) fn location(error: DiscoverPartyLocationsError) -> OperationError {
    match error {
        error @ DiscoverPartyLocationsError::InvalidInput => {
            OperationError::InvalidArguments(Box::new(error))
        }
        error @ (DiscoverPartyLocationsError::Read(_)
        | DiscoverPartyLocationsError::Transaction(_)) => {
            OperationError::Unavailable(Box::new(error))
        }
        error @ (DiscoverPartyLocationsError::InvalidState(_)
        | DiscoverPartyLocationsError::InconsistentSnapshot) => {
            OperationError::Internal(Box::new(error))
        }
    }
}

pub(super) fn overview(error: GetPublicCatalogueOverviewError) -> OperationError {
    match error {
        error @ (GetPublicCatalogueOverviewError::NotYetCalculated
        | GetPublicCatalogueOverviewError::ReaderTemporarilyUnavailable { .. }
        | GetPublicCatalogueOverviewError::BeginTransaction { .. }
        | GetPublicCatalogueOverviewError::CommitTransaction { .. }) => {
            OperationError::Unavailable(Box::new(error))
        }
        error @ GetPublicCatalogueOverviewError::ReaderInvalidReadModel { .. } => {
            OperationError::Internal(Box::new(error))
        }
    }
}
